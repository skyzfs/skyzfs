# Design §5.1 "Reads leave the worker" (zfs-topology a45338f), read before M1.3j

1. A hole as written. Between the log's start moving and the index
   letting go, a reader that found a record below the new start reads
   it through `Log::read_payload`, which refuses a record below the
   floor (`BadPosition`) and a segment no longer in `live`
   (`MissingSegment`); `Log::reclaim` changes both when the start
   moves. The store fails the volume on a failed read. So "only two
   points" does not hold for the log as built: either the reader's
   read of the log does not consult the floor and the live table (it
   resolves file and offset while it holds the index lock), or the
   log forgets the segments only at the second point.
2. The wait must be for the readers under way when it began, not for
   "no reader". Reads follow each other without a gap under load; a
   wait for a count of zero can starve, and the worker waiting is
   every write and flush waiting.
3. "A write a read has shown stays applied" holds, and I see no way
   it fails with several readers for reads that do not overlap in
   time: per block the index only moves from a record to a newer
   record or to the home image that has that record applied. It needs
   the lookup of a read's whole range under one hold and the insert
   of a write's whole range under one hold.
4. The failure mark: a reader that finds a read error must not send
   its failed completion before the worker has marked the log, since
   the mark in the log is the worker's.
5. Discards are not in the index and change nothing a read sees; no
   hole there.

# M1.3j (skyzfs/skyzfs#29): reads leave the worker

Reviewed: `origin/zfs-topology` ... `origin/skyzfs-coding` 78c204b.

## What was done

- Diff of log.rs, format.rs, store.rs and status.rs read before the
  description. fmt, clippy -D warnings, tests of skyzfs-local,
  skyzfs-ublk, skyzfsd: clean, all pass; the store's 210 tests four
  times in a row.
- One temporary test of the failure order; three plants of my own.
- Not done: no kernel run; the tests' file not read line by line; the
  description not compared claim by claim.

## Findings

1. Blocking. A read is answered `Failed` before the log is marked.
   `Core::fail` sets the flag the reader looks at, then marks the log
   (the observer, then the marker file and two syncs). A read that
   begins in between is refused by the reader, which sends the
   completion itself. Temporary test: the volume fails on the worker,
   the observer is held, a read is submitted: one completion,
   `Err(Failed)`, no marker in the directory. With the flag set after
   the mark the same read is answered `Ok`. Before this package no
   completion could leave before the mark.
2. Blocking for a green merge. `python3 tests/bench/test_skz_time.py`
   fails at this head (171 keys where 153 are asked; 2 failures). CI's
   lint job runs it. The benchmark's script also does not know the
   fifteenth activity, so its worker identity and its read tables do
   not hold for a version 6 status.
3. Not blocking. "The pin from just before the lookup" has no test:
   with the pin taken after the lookup all 210 tests pass.
4. Not blocking. A read of a block in a record that is not durable
   reads the record's whole payload (up to 1 MiB for 4 KiB). Counted
   in the status; the cost is not in the description's numbers.
5. Not blocking. A version 5 file is printed with the eighteen new
   keys as zeros.

## Agreed with

- The pin's generations: a reader that entered before a barrier began
  is waited for, one that entered after is not; both under one mutex.
  Plant "misses the current generation": caught, 4 tests.
- The segments beside the index are changed in the same hold at the
  insert, at the step's end and at the open. Plant "insert without
  following": caught, 4 tests.
- The durable position is told after the sync returned; the open
  syncs every file before it calls anything durable.
- No write or deallocation in the home image before the first
  barrier.
- Status version 6: 1568 bytes, the pump's fields 48 bytes further,
  version 5 still read.

## Second look (head 9a3f5ca)

- 1 dropped. The flag is set after the marker was tried (60e4c7c);
  the new test holds the observer and its mutant is caught.
- 2 dropped. `test_skz_time.py`: 24 tests pass; the keys the script
  reads are in status.rs.
- 3 kept in part, not blocking. The mutant takes the pin after the
  read's pause. A pin between the lookup and the pause (my plant) still
  passes all 212 tests. No hook can stand between the two today.
- 4 dropped. The description says the cost.
- `Mutants` at 64 bits: no bit changed; bit 24 was free.
- fmt, clippy clean; skyzfs-local 212 pass, 3 ignored.
